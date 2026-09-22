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
    /// A semantic content root is not a canonical `sha256:<64 hex>` digest.
    SemanticContentRootInvalid,
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
            Self::SemanticContentRootInvalid => "SEMANTIC_CONTENT_ROOT_INVALID",
        }
    }
}

impl fmt::Display for SearchCorpusGenerationIdentityValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code_v1())
    }
}

impl std::error::Error for SearchCorpusGenerationIdentityValidationErrorV1 {}

/// The content roots a sealed semantic generation carries (QI-BB-028):
/// the row root over every sealed row, vectors included, and the
/// cluster-membership root.
///
/// The manifest digest names what the producer asked for; these name what
/// the search plane actually sealed. Two state roots that sealed the same
/// source manifest with different embeddings share the digest but not the
/// roots, so activation names the roots and the plane refuses a physical
/// generation whose roots differ (`SEMANTIC_ROW_ROOT_MISMATCH`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticContentRootsV1 {
    pub row_root_digest: String,
    pub membership_root_digest: String,
}

fn is_canonical_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl SemanticContentRootsV1 {
    /// Both roots are canonical `sha256:<64 lowercase hex>` digests.
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        is_canonical_sha256_digest(&self.row_root_digest)
            && is_canonical_sha256_digest(&self.membership_root_digest)
    }
}

const SEMANTIC_CONTENT_ROOTS_V1_FIELDS: &[&str] = &["row_root_digest", "membership_root_digest"];

impl Serialize for SemanticContentRootsV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticContentRootsV1", 2)?;
        state.serialize_field("row_root_digest", &self.row_root_digest)?;
        state.serialize_field("membership_root_digest", &self.membership_root_digest)?;
        state.end()
    }
}

struct SemanticContentRootsV1Visitor;

impl<'de> Visitor<'de> for SemanticContentRootsV1Visitor {
    type Value = SemanticContentRootsV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticContentRootsV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut row_root_digest: Option<String> = None;
        let mut membership_root_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "row_root_digest" => {
                    if row_root_digest.is_some() {
                        return Err(de::Error::duplicate_field("row_root_digest"));
                    }
                    row_root_digest = Some(map.next_value()?);
                }
                "membership_root_digest" => {
                    if membership_root_digest.is_some() {
                        return Err(de::Error::duplicate_field("membership_root_digest"));
                    }
                    membership_root_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_CONTENT_ROOTS_V1_FIELDS,
                    ));
                }
            }
        }
        let roots = SemanticContentRootsV1 {
            row_root_digest: row_root_digest
                .ok_or_else(|| de::Error::missing_field("row_root_digest"))?,
            membership_root_digest: membership_root_digest
                .ok_or_else(|| de::Error::missing_field("membership_root_digest"))?,
        };
        if !roots.is_canonical_v1() {
            return Err(de::Error::custom(
                "semantic content roots must be canonical sha256 digests",
            ));
        }
        Ok(roots)
    }
}

impl<'de> Deserialize<'de> for SemanticContentRootsV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticContentRootsV1",
            SEMANTIC_CONTENT_ROOTS_V1_FIELDS,
            SemanticContentRootsV1Visitor,
        )
    }
}

/// Complete reader-visible generation identity for a search corpus.
///
/// The two records deliberately stay separate so each plane keeps its typed
/// track identity, while this composite makes lexical-only activation
/// unrepresentable on the production activation path. The semantic content
/// roots attest what the semantic generation actually sealed (QI-BB-028):
/// an activation names them, the plane refuses a physical generation whose
/// roots differ, and the activation CAS compares them like every other
/// field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationIdentityV1 {
    pub lexical: GenerationSnapshot,
    pub semantic: GenerationSnapshot,
    pub semantic_content: SemanticContentRootsV1,
}

impl SearchCorpusGenerationIdentityV1 {
    pub fn validate_v1(&self) -> Result<(), SearchCorpusGenerationIdentityValidationErrorV1> {
        if !self.semantic_content.is_canonical_v1() {
            return Err(
                SearchCorpusGenerationIdentityValidationErrorV1::SemanticContentRootInvalid,
            );
        }
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

/// Validation failure for a composite search-corpus activation request.
///
/// This is the wire-contract authority for the candidate/expectation
/// relationship. SDK and runtime consumers must delegate here instead of
/// independently re-encoding the CAS invariants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchCorpusActivationValidationErrorV1 {
    CandidateIdentity(SearchCorpusGenerationIdentityValidationErrorV1),
    ExpectedActiveIdentity(SearchCorpusGenerationIdentityValidationErrorV1),
    RepoMismatch,
    RevisionMismatch,
    CandidateGenerationMustAdvanceExpectedActive,
}

impl SearchCorpusActivationValidationErrorV1 {
    #[must_use]
    pub const fn code_v1(self) -> &'static str {
        match self {
            Self::CandidateIdentity(_) => "CANDIDATE_IDENTITY_INVALID",
            Self::ExpectedActiveIdentity(_) => "EXPECTED_ACTIVE_IDENTITY_INVALID",
            Self::RepoMismatch => "REPO_MISMATCH",
            Self::RevisionMismatch => "REVISION_MISMATCH",
            Self::CandidateGenerationMustAdvanceExpectedActive => {
                "CANDIDATE_GENERATION_MUST_ADVANCE_EXPECTED_ACTIVE"
            }
        }
    }
}

impl fmt::Display for SearchCorpusActivationValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateIdentity(error) | Self::ExpectedActiveIdentity(error) => {
                write!(formatter, "{}: {error}", self.code_v1())
            }
            Self::RepoMismatch
            | Self::RevisionMismatch
            | Self::CandidateGenerationMustAdvanceExpectedActive => {
                formatter.write_str(self.code_v1())
            }
        }
    }
}

impl std::error::Error for SearchCorpusActivationValidationErrorV1 {}

impl SearchPlaneActivateSearchCorpusGenerationCasRequest {
    pub fn validate_v1(&self) -> Result<(), SearchCorpusActivationValidationErrorV1> {
        self.candidate
            .validate_v1()
            .map_err(SearchCorpusActivationValidationErrorV1::CandidateIdentity)?;
        Self::validate_expected_active_v1(&self.candidate.lexical, self.expected_active.as_ref())
    }

    /// The CAS expectation's own invariants against the candidate's scope
    /// and generation, which a producer knows before it publishes: the
    /// expectation is a valid identity, names the candidate's pair, and
    /// the candidate strictly advances it. This is the one authority for
    /// those invariants; [`Self::validate_v1`] runs it after the candidate
    /// identity check, and a producer may run it before the candidate's
    /// content roots exist.
    pub fn validate_expected_active_v1(
        candidate_scope: &GenerationSnapshot,
        expected_active: Option<&SearchCorpusGenerationIdentityV1>,
    ) -> Result<(), SearchCorpusActivationValidationErrorV1> {
        let Some(expected_active) = expected_active else {
            return Ok(());
        };
        expected_active
            .validate_v1()
            .map_err(SearchCorpusActivationValidationErrorV1::ExpectedActiveIdentity)?;
        if candidate_scope.repo_id != expected_active.lexical.repo_id {
            return Err(SearchCorpusActivationValidationErrorV1::RepoMismatch);
        }
        if candidate_scope.revision_id != expected_active.lexical.revision_id {
            return Err(SearchCorpusActivationValidationErrorV1::RevisionMismatch);
        }
        if candidate_scope.manifest_generation.get()
            <= expected_active.lexical.manifest_generation.get()
        {
            return Err(
                SearchCorpusActivationValidationErrorV1::CandidateGenerationMustAdvanceExpectedActive,
            );
        }
        Ok(())
    }
}

/// Receipt of one successful composite search-corpus activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSearchCorpusActivationCasAck {
    pub active: SearchCorpusGenerationIdentityV1,
    pub previous_sealed_active: Option<SearchCorpusGenerationIdentityV1>,
}

const SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS: &[&str] =
    &["lexical", "semantic", "semantic_content"];
const SEARCH_PLANE_ACTIVATE_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS: &[&str] =
    &["candidate", "expected_active"];
const SEARCH_PLANE_SEARCH_CORPUS_ACTIVATION_CAS_ACK_FIELDS: &[&str] =
    &["active", "previous_sealed_active"];

impl Serialize for SearchCorpusGenerationIdentityV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusGenerationIdentityV1", 3)?;
        state.serialize_field("lexical", &self.lexical)?;
        state.serialize_field("semantic", &self.semantic)?;
        state.serialize_field("semantic_content", &self.semantic_content)?;
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
        let mut semantic_content: Option<SemanticContentRootsV1> = None;
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
                "semantic_content" => {
                    if semantic_content.is_some() {
                        return Err(de::Error::duplicate_field("semantic_content"));
                    }
                    semantic_content = Some(map.next_value()?);
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
            semantic_content: semantic_content
                .ok_or_else(|| de::Error::missing_field("semantic_content"))?,
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

/// Explicit compare-and-swap rollback request for one complete search corpus.
///
/// Both the expected head and rollback target carry exact lexical plus
/// semantic identities. A single-track rollback cannot be represented on the
/// wire, and a stale operator cannot overwrite a concurrent activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneRollbackSearchCorpusGenerationCasRequest {
    pub expected_active: SearchCorpusGenerationIdentityV1,
    pub target: SearchCorpusGenerationIdentityV1,
}

/// Validation failure for a composite search-corpus rollback request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchCorpusRollbackValidationErrorV1 {
    ExpectedActiveIdentity(SearchCorpusGenerationIdentityValidationErrorV1),
    TargetIdentity(SearchCorpusGenerationIdentityValidationErrorV1),
    RepoMismatch,
    RevisionMismatch,
    TargetGenerationMustPrecedeExpectedActive,
}

impl SearchCorpusRollbackValidationErrorV1 {
    #[must_use]
    pub const fn code_v1(self) -> &'static str {
        match self {
            Self::ExpectedActiveIdentity(_) => "EXPECTED_ACTIVE_IDENTITY_INVALID",
            Self::TargetIdentity(_) => "TARGET_IDENTITY_INVALID",
            Self::RepoMismatch => "REPO_MISMATCH",
            Self::RevisionMismatch => "REVISION_MISMATCH",
            Self::TargetGenerationMustPrecedeExpectedActive => {
                "TARGET_GENERATION_MUST_PRECEDE_EXPECTED_ACTIVE"
            }
        }
    }
}

impl fmt::Display for SearchCorpusRollbackValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedActiveIdentity(error) | Self::TargetIdentity(error) => {
                write!(formatter, "{}: {error}", self.code_v1())
            }
            Self::RepoMismatch
            | Self::RevisionMismatch
            | Self::TargetGenerationMustPrecedeExpectedActive => {
                formatter.write_str(self.code_v1())
            }
        }
    }
}

impl std::error::Error for SearchCorpusRollbackValidationErrorV1 {}

impl SearchPlaneRollbackSearchCorpusGenerationCasRequest {
    pub fn validate_v1(&self) -> Result<(), SearchCorpusRollbackValidationErrorV1> {
        self.expected_active
            .validate_v1()
            .map_err(SearchCorpusRollbackValidationErrorV1::ExpectedActiveIdentity)?;
        self.target
            .validate_v1()
            .map_err(SearchCorpusRollbackValidationErrorV1::TargetIdentity)?;
        if self.expected_active.lexical.repo_id != self.target.lexical.repo_id {
            return Err(SearchCorpusRollbackValidationErrorV1::RepoMismatch);
        }
        if self.expected_active.lexical.revision_id != self.target.lexical.revision_id {
            return Err(SearchCorpusRollbackValidationErrorV1::RevisionMismatch);
        }
        if self.target.lexical.manifest_generation.get()
            >= self.expected_active.lexical.manifest_generation.get()
        {
            return Err(
                SearchCorpusRollbackValidationErrorV1::TargetGenerationMustPrecedeExpectedActive,
            );
        }
        Ok(())
    }
}

/// Receipt of one successful composite search-corpus rollback CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSearchCorpusRollbackCasAck {
    pub active: SearchCorpusGenerationIdentityV1,
    pub previous_sealed_active: SearchCorpusGenerationIdentityV1,
}

const SEARCH_PLANE_ROLLBACK_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS: &[&str] =
    &["expected_active", "target"];
const SEARCH_PLANE_SEARCH_CORPUS_ROLLBACK_CAS_ACK_FIELDS: &[&str] =
    &["active", "previous_sealed_active"];

impl Serialize for SearchPlaneRollbackSearchCorpusGenerationCasRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer
            .serialize_struct("SearchPlaneRollbackSearchCorpusGenerationCasRequest", 2)?;
        state.serialize_field("expected_active", &self.expected_active)?;
        state.serialize_field("target", &self.target)?;
        state.end()
    }
}

struct SearchPlaneRollbackSearchCorpusGenerationCasRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneRollbackSearchCorpusGenerationCasRequestVisitor {
    type Value = SearchPlaneRollbackSearchCorpusGenerationCasRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneRollbackSearchCorpusGenerationCasRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut expected_active = None;
        let mut target = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "expected_active" => {
                    if expected_active.is_some() {
                        return Err(de::Error::duplicate_field("expected_active"));
                    }
                    expected_active = Some(map.next_value()?);
                }
                "target" => {
                    if target.is_some() {
                        return Err(de::Error::duplicate_field("target"));
                    }
                    target = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_ROLLBACK_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(Self::Value {
            expected_active: expected_active
                .ok_or_else(|| de::Error::missing_field("expected_active"))?,
            target: target.ok_or_else(|| de::Error::missing_field("target"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneRollbackSearchCorpusGenerationCasRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneRollbackSearchCorpusGenerationCasRequest",
            SEARCH_PLANE_ROLLBACK_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS,
            SearchPlaneRollbackSearchCorpusGenerationCasRequestVisitor,
        )
    }
}

impl Serialize for SearchPlaneSearchCorpusRollbackCasAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneSearchCorpusRollbackCasAck", 2)?;
        state.serialize_field("active", &self.active)?;
        state.serialize_field("previous_sealed_active", &self.previous_sealed_active)?;
        state.end()
    }
}

struct SearchPlaneSearchCorpusRollbackCasAckVisitor;

impl<'de> Visitor<'de> for SearchPlaneSearchCorpusRollbackCasAckVisitor {
    type Value = SearchPlaneSearchCorpusRollbackCasAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSearchCorpusRollbackCasAck map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut active = None;
        let mut previous_sealed_active = None;
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
                        SEARCH_PLANE_SEARCH_CORPUS_ROLLBACK_CAS_ACK_FIELDS,
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

impl<'de> Deserialize<'de> for SearchPlaneSearchCorpusRollbackCasAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSearchCorpusRollbackCasAck",
            SEARCH_PLANE_SEARCH_CORPUS_ROLLBACK_CAS_ACK_FIELDS,
            SearchPlaneSearchCorpusRollbackCasAckVisitor,
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
/// `(repo, revision)` pair.
///
/// `tracks` is order-stable (lexical before semantic per
/// `SearchPlaneTrackKind` declaration order). `semantic_content` is the
/// active semantic generation's content roots (QI-BB-028), present exactly
/// when the pair has an active composite root, so an activator can name
/// the roots of the head it expects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationStatusReport {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub tracks: Vec<TrackReadinessRecord>,
    pub semantic_content: Option<SemanticContentRootsV1>,
}

const GENERATION_STATUS_REPORT_FIELDS: &[&str] =
    &["repo_id", "revision_id", "tracks", "semantic_content"];

impl Serialize for GenerationStatusReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("GenerationStatusReport", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("tracks", &self.tracks)?;
        state.serialize_field("semantic_content", &self.semantic_content)?;
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
        let mut semantic_content: Option<Option<SemanticContentRootsV1>> = None;
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
                "semantic_content" => {
                    if semantic_content.is_some() {
                        return Err(de::Error::duplicate_field("semantic_content"));
                    }
                    semantic_content = Some(map.next_value()?);
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
            semantic_content: semantic_content
                .ok_or_else(|| de::Error::missing_field("semantic_content"))?,
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
// S21-10 process readiness (P09): process-wide readiness DTO, deliberately
// distinct from repository generation status above.
// =============================================================================

/// Ask the daemon for its process-wide readiness synthesis.
///
/// A readiness probe takes no parameters; the struct exists so the request
/// has a typed payload that decodes fail-closed like every other one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessReadinessRequest;

const PROCESS_READINESS_REQUEST_FIELDS: &[&str] = &[];

impl Serialize for ProcessReadinessRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer
            .serialize_struct("ProcessReadinessRequest", 0)?
            .end()
    }
}

struct ProcessReadinessRequestVisitor;

impl<'de> Visitor<'de> for ProcessReadinessRequestVisitor {
    type Value = ProcessReadinessRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an empty ProcessReadinessRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        if let Some(key) = map.next_key::<String>()? {
            return Err(de::Error::unknown_field(
                &key,
                PROCESS_READINESS_REQUEST_FIELDS,
            ));
        }
        Ok(ProcessReadinessRequest)
    }
}

impl<'de> Deserialize<'de> for ProcessReadinessRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ProcessReadinessRequest",
            PROCESS_READINESS_REQUEST_FIELDS,
            ProcessReadinessRequestVisitor,
        )
    }
}

/// The supervisor phase a process reports, mirroring the runtime's
/// supervised lifecycle (S21-09). Closed vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessReadinessPhaseV1 {
    Starting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

impl ProcessReadinessPhaseV1 {
    const VARIANTS: &'static [&'static str] = &["starting", "ready", "draining", "stopped", "failed"];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

impl Serialize for ProcessReadinessPhaseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct ProcessReadinessPhaseV1Visitor;

impl<'de> Visitor<'de> for ProcessReadinessPhaseV1Visitor {
    type Value = ProcessReadinessPhaseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a process readiness phase code")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Self::Value::VARIANTS
            .iter()
            .position(|candidate| *candidate == value)
            .map(|index| match index {
                0 => Self::Value::Starting,
                1 => Self::Value::Ready,
                2 => Self::Value::Draining,
                3 => Self::Value::Stopped,
                _ => Self::Value::Failed,
            })
            .ok_or_else(|| E::unknown_variant(value, Self::Value::VARIANTS))
    }
}

impl<'de> Deserialize<'de> for ProcessReadinessPhaseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ProcessReadinessPhaseV1Visitor)
    }
}

/// What the provider profile claims about a provider executor's role in
/// readiness (S21-08). Closed vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessProviderClaimV1 {
    Required,
    Degraded,
    Disabled,
}

impl ProcessProviderClaimV1 {
    const VARIANTS: &'static [&'static str] = &["required", "degraded", "disabled"];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Degraded => "degraded",
            Self::Disabled => "disabled",
        }
    }
}

impl Serialize for ProcessProviderClaimV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct ProcessProviderClaimV1Visitor;

impl<'de> Visitor<'de> for ProcessProviderClaimV1Visitor {
    type Value = ProcessProviderClaimV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a provider readiness claim code")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "required" => Ok(Self::Value::Required),
            "degraded" => Ok(Self::Value::Degraded),
            "disabled" => Ok(Self::Value::Disabled),
            _ => Err(E::unknown_variant(value, Self::Value::VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for ProcessProviderClaimV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ProcessProviderClaimV1Visitor)
    }
}

/// One provider executor's readiness as its profile claims it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessProviderReadinessV1 {
    /// The profile's claim: `required` providers gate readiness,
    /// `degraded` ones are recorded but do not gate, `disabled` ones are
    /// not expected to run.
    pub claim: ProcessProviderClaimV1,
    /// Whether the executor is currently healthy.
    pub healthy: bool,
}

const PROCESS_PROVIDER_READINESS_FIELDS: &[&str] = &["claim", "healthy"];

impl Serialize for ProcessProviderReadinessV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProcessProviderReadinessV1", 2)?;
        state.serialize_field("claim", &self.claim)?;
        state.serialize_field("healthy", &self.healthy)?;
        state.end()
    }
}

struct ProcessProviderReadinessV1Visitor;

impl<'de> Visitor<'de> for ProcessProviderReadinessV1Visitor {
    type Value = ProcessProviderReadinessV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ProcessProviderReadinessV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut claim = None;
        let mut healthy = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "claim" => {
                    if claim.is_some() {
                        return Err(de::Error::duplicate_field("claim"));
                    }
                    claim = Some(map.next_value()?);
                }
                "healthy" => {
                    if healthy.is_some() {
                        return Err(de::Error::duplicate_field("healthy"));
                    }
                    healthy = Some(map.next_value()?);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let claim = claim.ok_or_else(|| de::Error::missing_field("claim"))?;
        let healthy = healthy.ok_or_else(|| de::Error::missing_field("healthy"))?;
        Ok(Self::Value { claim, healthy })
    }
}

impl<'de> Deserialize<'de> for ProcessProviderReadinessV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ProcessProviderReadinessV1",
            PROCESS_PROVIDER_READINESS_FIELDS,
            ProcessProviderReadinessV1Visitor,
        )
    }
}

/// The health of every required process component one readiness
/// synthesis consults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessComponentsHealthV1 {
    /// The query plane's accept loop is alive and accepting.
    pub query_plane: bool,
    /// The control plane's accept loop is alive and accepting.
    pub control_plane: bool,
    /// The ingest plane's accept loop is alive and accepting.
    pub ingest_plane: bool,
    /// The maintenance heartbeat is fresh.
    pub maintenance_heartbeat: bool,
    /// The required backend's open proof succeeded.
    pub required_backend: bool,
    /// The provider executor as its profile claims it.
    pub provider: ProcessProviderReadinessV1,
}

const PROCESS_COMPONENTS_HEALTH_FIELDS: &[&str] = &[
    "query_plane",
    "control_plane",
    "ingest_plane",
    "maintenance_heartbeat",
    "required_backend",
    "provider",
];

impl Serialize for ProcessComponentsHealthV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProcessComponentsHealthV1", 6)?;
        state.serialize_field("query_plane", &self.query_plane)?;
        state.serialize_field("control_plane", &self.control_plane)?;
        state.serialize_field("ingest_plane", &self.ingest_plane)?;
        state.serialize_field("maintenance_heartbeat", &self.maintenance_heartbeat)?;
        state.serialize_field("required_backend", &self.required_backend)?;
        state.serialize_field("provider", &self.provider)?;
        state.end()
    }
}

struct ProcessComponentsHealthV1Visitor;

impl<'de> Visitor<'de> for ProcessComponentsHealthV1Visitor {
    type Value = ProcessComponentsHealthV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ProcessComponentsHealthV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut query_plane = None;
        let mut control_plane = None;
        let mut ingest_plane = None;
        let mut maintenance_heartbeat = None;
        let mut required_backend = None;
        let mut provider = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query_plane" => {
                    if query_plane.is_some() {
                        return Err(de::Error::duplicate_field("query_plane"));
                    }
                    query_plane = Some(map.next_value()?);
                }
                "control_plane" => {
                    if control_plane.is_some() {
                        return Err(de::Error::duplicate_field("control_plane"));
                    }
                    control_plane = Some(map.next_value()?);
                }
                "ingest_plane" => {
                    if ingest_plane.is_some() {
                        return Err(de::Error::duplicate_field("ingest_plane"));
                    }
                    ingest_plane = Some(map.next_value()?);
                }
                "maintenance_heartbeat" => {
                    if maintenance_heartbeat.is_some() {
                        return Err(de::Error::duplicate_field("maintenance_heartbeat"));
                    }
                    maintenance_heartbeat = Some(map.next_value()?);
                }
                "required_backend" => {
                    if required_backend.is_some() {
                        return Err(de::Error::duplicate_field("required_backend"));
                    }
                    required_backend = Some(map.next_value()?);
                }
                "provider" => {
                    if provider.is_some() {
                        return Err(de::Error::duplicate_field("provider"));
                    }
                    provider = Some(map.next_value()?);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let query_plane = query_plane.ok_or_else(|| de::Error::missing_field("query_plane"))?;
        let control_plane =
            control_plane.ok_or_else(|| de::Error::missing_field("control_plane"))?;
        let ingest_plane = ingest_plane.ok_or_else(|| de::Error::missing_field("ingest_plane"))?;
        let maintenance_heartbeat = maintenance_heartbeat
            .ok_or_else(|| de::Error::missing_field("maintenance_heartbeat"))?;
        let required_backend = required_backend
            .ok_or_else(|| de::Error::missing_field("required_backend"))?;
        let provider = provider.ok_or_else(|| de::Error::missing_field("provider"))?;
        Ok(Self::Value {
            query_plane,
            control_plane,
            ingest_plane,
            maintenance_heartbeat,
            required_backend,
            provider,
        })
    }
}

impl<'de> Deserialize<'de> for ProcessComponentsHealthV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ProcessComponentsHealthV1",
            PROCESS_COMPONENTS_HEALTH_FIELDS,
            ProcessComponentsHealthV1Visitor,
        )
    }
}

/// Why a process is not ready. Closed vocabulary: every reason a
/// readiness synthesis can report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessReadinessReasonV1 {
    SupervisorNotReady,
    QueryPlaneUnhealthy,
    ControlPlaneUnhealthy,
    IngestPlaneUnhealthy,
    MaintenanceHeartbeatStale,
    RequiredBackendOpenUnproven,
    ProviderRequiredUnhealthy,
    ActiveCandidateIntegrityFailed,
}

impl ProcessReadinessReasonV1 {
    const VARIANTS: &'static [&'static str] = &[
        "supervisor_not_ready",
        "query_plane_unhealthy",
        "control_plane_unhealthy",
        "ingest_plane_unhealthy",
        "maintenance_heartbeat_stale",
        "required_backend_open_unproven",
        "provider_required_unhealthy",
        "active_candidate_integrity_failed",
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SupervisorNotReady => "supervisor_not_ready",
            Self::QueryPlaneUnhealthy => "query_plane_unhealthy",
            Self::ControlPlaneUnhealthy => "control_plane_unhealthy",
            Self::IngestPlaneUnhealthy => "ingest_plane_unhealthy",
            Self::MaintenanceHeartbeatStale => "maintenance_heartbeat_stale",
            Self::RequiredBackendOpenUnproven => "required_backend_open_unproven",
            Self::ProviderRequiredUnhealthy => "provider_required_unhealthy",
            Self::ActiveCandidateIntegrityFailed => "active_candidate_integrity_failed",
        }
    }
}

impl Serialize for ProcessReadinessReasonV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct ProcessReadinessReasonV1Visitor;

impl<'de> Visitor<'de> for ProcessReadinessReasonV1Visitor {
    type Value = ProcessReadinessReasonV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a process readiness reason code")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let reason = match value {
            "supervisor_not_ready" => Self::Value::SupervisorNotReady,
            "query_plane_unhealthy" => Self::Value::QueryPlaneUnhealthy,
            "control_plane_unhealthy" => Self::Value::ControlPlaneUnhealthy,
            "ingest_plane_unhealthy" => Self::Value::IngestPlaneUnhealthy,
            "maintenance_heartbeat_stale" => Self::Value::MaintenanceHeartbeatStale,
            "required_backend_open_unproven" => Self::Value::RequiredBackendOpenUnproven,
            "provider_required_unhealthy" => Self::Value::ProviderRequiredUnhealthy,
            "active_candidate_integrity_failed" => Self::Value::ActiveCandidateIntegrityFailed,
            _ => return Err(E::unknown_variant(value, Self::Value::VARIANTS)),
        };
        Ok(reason)
    }
}

impl<'de> Deserialize<'de> for ProcessReadinessReasonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ProcessReadinessReasonV1Visitor)
    }
}

/// Process-wide readiness (S21-10 / D-READY-01).
///
/// Deliberately distinct from repository generation status: `ready=true`
/// means the *process* can serve — supervisor ready, every required
/// plane accepting, maintenance heartbeat fresh, required backend open
/// proven, required provider executor healthy — and is legitimate with
/// zero active repositories. The candidate integrity gate applies only
/// while an active candidate exists (`active_candidate_integrity=None`
/// means none does).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessReadinessV1 {
    /// The conjunction of every required component below.
    pub ready: bool,
    /// The supervisor's phase as the readiness synthesis saw it.
    pub supervisor_phase: ProcessReadinessPhaseV1,
    /// Required plane, maintenance, backend and provider health.
    pub components: ProcessComponentsHealthV1,
    /// `Some(false)` names a failed active candidate integrity gate;
    /// `None` means no active candidate exists and the gate does not
    /// apply.
    pub active_candidate_integrity: Option<bool>,
    /// How many repositories currently have an active generation. Zero
    /// is a legitimate ready state.
    pub active_repositories: u64,
    /// Every failed required component, in fixed evaluation order. Empty
    /// when `ready=true`.
    pub not_ready_reasons: Vec<ProcessReadinessReasonV1>,
}

const PROCESS_READINESS_FIELDS: &[&str] = &[
    "ready",
    "supervisor_phase",
    "components",
    "active_candidate_integrity",
    "active_repositories",
    "not_ready_reasons",
];

impl Serialize for ProcessReadinessV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProcessReadinessV1", 6)?;
        state.serialize_field("ready", &self.ready)?;
        state.serialize_field("supervisor_phase", &self.supervisor_phase)?;
        state.serialize_field("components", &self.components)?;
        state.serialize_field(
            "active_candidate_integrity",
            &self.active_candidate_integrity,
        )?;
        state.serialize_field("active_repositories", &self.active_repositories)?;
        state.serialize_field("not_ready_reasons", &self.not_ready_reasons)?;
        state.end()
    }
}

struct ProcessReadinessV1Visitor;

impl<'de> Visitor<'de> for ProcessReadinessV1Visitor {
    type Value = ProcessReadinessV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ProcessReadinessV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut ready = None;
        let mut supervisor_phase = None;
        let mut components = None;
        let mut active_candidate_integrity = None;
        let mut active_candidate_integrity_seen = false;
        let mut active_repositories = None;
        let mut not_ready_reasons = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "ready" => {
                    if ready.is_some() {
                        return Err(de::Error::duplicate_field("ready"));
                    }
                    ready = Some(map.next_value()?);
                }
                "supervisor_phase" => {
                    if supervisor_phase.is_some() {
                        return Err(de::Error::duplicate_field("supervisor_phase"));
                    }
                    supervisor_phase = Some(map.next_value()?);
                }
                "components" => {
                    if components.is_some() {
                        return Err(de::Error::duplicate_field("components"));
                    }
                    components = Some(map.next_value()?);
                }
                "active_candidate_integrity" => {
                    if active_candidate_integrity_seen {
                        return Err(de::Error::duplicate_field("active_candidate_integrity"));
                    }
                    active_candidate_integrity_seen = true;
                    active_candidate_integrity = map.next_value()?;
                }
                "active_repositories" => {
                    if active_repositories.is_some() {
                        return Err(de::Error::duplicate_field("active_repositories"));
                    }
                    active_repositories = Some(map.next_value()?);
                }
                "not_ready_reasons" => {
                    if not_ready_reasons.is_some() {
                        return Err(de::Error::duplicate_field("not_ready_reasons"));
                    }
                    not_ready_reasons = Some(map.next_value()?);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let ready = ready.ok_or_else(|| de::Error::missing_field("ready"))?;
        let supervisor_phase =
            supervisor_phase.ok_or_else(|| de::Error::missing_field("supervisor_phase"))?;
        let components = components.ok_or_else(|| de::Error::missing_field("components"))?;
        let active_repositories = active_repositories
            .ok_or_else(|| de::Error::missing_field("active_repositories"))?;
        let not_ready_reasons = not_ready_reasons
            .ok_or_else(|| de::Error::missing_field("not_ready_reasons"))?;
        Ok(Self::Value {
            ready,
            supervisor_phase,
            components,
            active_candidate_integrity,
            active_repositories,
            not_ready_reasons,
        })
    }
}

impl<'de> Deserialize<'de> for ProcessReadinessV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ProcessReadinessV1",
            PROCESS_READINESS_FIELDS,
            ProcessReadinessV1Visitor,
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
        RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
    }

    fn fixture_rev() -> RevisionId {
        RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
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

    fn fixture_roots(generation: u64) -> SemanticContentRootsV1 {
        SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{generation:0>64x}"),
            membership_root_digest: format!("sha256:{:0>64x}", generation.saturating_add(1000)),
        }
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
            semantic_content: fixture_roots(generation),
        }
    }

    /// The semantic content roots are part of the identity (QI-BB-028).
    ///
    /// They round-trip, an identity naming a non-canonical root is
    /// refused by validation, a wire identity without them is refused
    /// by the decoder, and two identities that differ only in their roots
    /// are different identities for the activation CAS.
    #[test]
    fn search_corpus_identity_carries_and_validates_semantic_content_roots_v1() {
        let identity = corpus_identity(3, "digest-3");
        let bytes = encode(&identity).expect("encode identity");
        let decoded = decode::<SearchCorpusGenerationIdentityV1>(&bytes).expect("decode identity");
        assert_eq!(decoded, identity);

        let mut other_roots = identity.clone();
        other_roots.semantic_content = fixture_roots(4);
        assert_ne!(other_roots, identity, "the roots are identity");

        let mut invalid = identity;
        invalid.semantic_content.row_root_digest = "not-a-digest".to_string();
        assert_eq!(
            invalid.validate_v1(),
            Err(SearchCorpusGenerationIdentityValidationErrorV1::SemanticContentRootInvalid)
        );
        let invalid_bytes = encode(&invalid).expect("encode invalid identity");
        assert!(
            decode::<SearchCorpusGenerationIdentityV1>(&invalid_bytes).is_err(),
            "a non-canonical root does not decode"
        );

        let without_roots = ciborium::value::Value::Map(vec![
            (
                ciborium::value::Value::Text("lexical".to_string()),
                ciborium::value::Value::Map(Vec::new()),
            ),
            (
                ciborium::value::Value::Text("semantic".to_string()),
                ciborium::value::Value::Map(Vec::new()),
            ),
        ]);
        let mut bytes = Vec::new();
        ciborium::into_writer(&without_roots, &mut bytes).expect("encode");
        assert!(
            decode::<SearchCorpusGenerationIdentityV1>(&bytes).is_err(),
            "an identity without semantic content roots is refused"
        );
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
        assert_eq!(request.validate_v1(), Ok(()));
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
    fn search_corpus_activation_validation_rejects_invalid_relation_v1() {
        let expected_active = corpus_identity(10, "digest-10");

        let mut malformed_candidate = corpus_identity(11, "digest-11");
        malformed_candidate.semantic.track = SearchPlaneTrackKind::Lexical;
        assert_eq!(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: malformed_candidate,
                expected_active: Some(expected_active.clone()),
            }
            .validate_v1(),
            Err(SearchCorpusActivationValidationErrorV1::CandidateIdentity(
                SearchCorpusGenerationIdentityValidationErrorV1::SemanticTrackRequired,
            ))
        );

        let mut malformed_expected = expected_active.clone();
        malformed_expected.semantic.track = SearchPlaneTrackKind::Lexical;
        assert_eq!(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: corpus_identity(11, "digest-11"),
                expected_active: Some(malformed_expected),
            }
            .validate_v1(),
            Err(
                SearchCorpusActivationValidationErrorV1::ExpectedActiveIdentity(
                    SearchCorpusGenerationIdentityValidationErrorV1::SemanticTrackRequired,
                ),
            )
        );

        let mut cross_repo = corpus_identity(11, "digest-11");
        cross_repo.lexical.repo_id =
            RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
        cross_repo.semantic.repo_id =
            RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
        assert_eq!(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: cross_repo,
                expected_active: Some(expected_active.clone()),
            }
            .validate_v1(),
            Err(SearchCorpusActivationValidationErrorV1::RepoMismatch)
        );

        let mut cross_revision = corpus_identity(11, "digest-11");
        cross_revision.lexical.revision_id = RevisionId::new("other-revision")
            .expect("static fixture ID satisfies canonical policy");
        cross_revision.semantic.revision_id = RevisionId::new("other-revision")
            .expect("static fixture ID satisfies canonical policy");
        assert_eq!(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: cross_revision,
                expected_active: Some(expected_active.clone()),
            }
            .validate_v1(),
            Err(SearchCorpusActivationValidationErrorV1::RevisionMismatch)
        );

        for generation in [9, 10] {
            assert_eq!(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: corpus_identity(generation, "non-advancing-digest"),
                    expected_active: Some(expected_active.clone()),
                }
                .validate_v1(),
                Err(
                    SearchCorpusActivationValidationErrorV1::CandidateGenerationMustAdvanceExpectedActive,
                )
            );
        }

        assert_eq!(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: corpus_identity(1, "first-activation"),
                expected_active: None,
            }
            .validate_v1(),
            Ok(())
        );
    }

    #[test]
    fn search_corpus_rollback_request_and_ack_round_trip() {
        let expected_active = corpus_identity(11, "digest-11");
        let target = corpus_identity(10, "digest-10");
        let request = SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: expected_active.clone(),
            target: target.clone(),
        };
        assert_eq!(request.validate_v1(), Ok(()));
        let Ok(bytes) = encode(&request) else {
            assert!(
                false,
                "failed to encode SearchPlaneRollbackSearchCorpusGenerationCasRequest"
            );
            return;
        };
        let Ok(decoded) = decode::<SearchPlaneRollbackSearchCorpusGenerationCasRequest>(&bytes)
        else {
            assert!(
                false,
                "failed to decode SearchPlaneRollbackSearchCorpusGenerationCasRequest"
            );
            return;
        };
        assert_eq!(decoded, request);

        let ack = SearchPlaneSearchCorpusRollbackCasAck {
            active: target,
            previous_sealed_active: expected_active,
        };
        let Ok(bytes) = encode(&ack) else {
            assert!(
                false,
                "failed to encode SearchPlaneSearchCorpusRollbackCasAck"
            );
            return;
        };
        let Ok(decoded) = decode::<SearchPlaneSearchCorpusRollbackCasAck>(&bytes) else {
            assert!(
                false,
                "failed to decode SearchPlaneSearchCorpusRollbackCasAck"
            );
            return;
        };
        assert_eq!(decoded, ack);
    }

    #[test]
    fn search_corpus_rollback_validation_rejects_invalid_relation_v1() {
        let expected_active = corpus_identity(11, "digest-11");

        let mut cross_repo = corpus_identity(10, "digest-10");
        cross_repo.lexical.repo_id =
            RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
        cross_repo.semantic.repo_id =
            RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
        assert_eq!(
            SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: expected_active.clone(),
                target: cross_repo,
            }
            .validate_v1(),
            Err(SearchCorpusRollbackValidationErrorV1::RepoMismatch)
        );

        let mut cross_revision = corpus_identity(10, "digest-10");
        cross_revision.lexical.revision_id = RevisionId::new("other-revision")
            .expect("static fixture ID satisfies canonical policy");
        cross_revision.semantic.revision_id = RevisionId::new("other-revision")
            .expect("static fixture ID satisfies canonical policy");
        assert_eq!(
            SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: expected_active.clone(),
                target: cross_revision,
            }
            .validate_v1(),
            Err(SearchCorpusRollbackValidationErrorV1::RevisionMismatch)
        );

        assert_eq!(
            SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: expected_active.clone(),
                target: corpus_identity(11, "digest-same-generation"),
            }
            .validate_v1(),
            Err(SearchCorpusRollbackValidationErrorV1::TargetGenerationMustPrecedeExpectedActive)
        );

        let mut malformed_target = corpus_identity(10, "digest-10");
        malformed_target.semantic.track = SearchPlaneTrackKind::Lexical;
        assert_eq!(
            SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active,
                target: malformed_target,
            }
            .validate_v1(),
            Err(SearchCorpusRollbackValidationErrorV1::TargetIdentity(
                SearchCorpusGenerationIdentityValidationErrorV1::SemanticTrackRequired
            ))
        );
    }

    #[test]
    fn search_corpus_rollback_request_rejects_unknown_and_missing_fields() {
        let unknown = serde_json::json!({
            "expected_active": serde_json::to_value(corpus_identity(11, "digest-11"))
                .expect("encode expected active fixture"),
            "target": serde_json::to_value(corpus_identity(10, "digest-10"))
                .expect("encode target fixture"),
            "unexpected": true,
        });
        assert!(SearchPlaneRollbackSearchCorpusGenerationCasRequest::deserialize(unknown).is_err());

        let missing = serde_json::json!({
            "expected_active": serde_json::to_value(corpus_identity(11, "digest-11"))
                .expect("encode expected active fixture"),
        });
        let Err(err) = SearchPlaneRollbackSearchCorpusGenerationCasRequest::deserialize(missing)
        else {
            assert!(false, "missing target unexpectedly deserialized");
            return;
        };
        assert!(err.to_string().contains("missing field `target`"));
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
            semantic_content: None,
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
            semantic_content: None,
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
