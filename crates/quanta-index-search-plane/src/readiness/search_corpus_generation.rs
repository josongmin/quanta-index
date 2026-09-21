//! The composite (lexical + semantic) search-corpus generation identity and
//! its persisted activation root shape.

use std::fmt;

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    SemanticContentRootsV1,
};
use quanta_index_core::CoreError;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// One immutable, query-visible lexical plus semantic generation identity,
/// with the content roots the semantic generation sealed (QI-BB-028).
///
/// Construction validates that both tracks name the exact same source
/// generation and that the roots are canonical digests. The fields
/// intentionally stay private: callers cannot create a lexical-only or
/// mixed-generation corpus identity by struct literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationV1 {
    lexical: GenerationSnapshot,
    semantic: GenerationSnapshot,
    semantic_content: SemanticContentRootsV1,
}

impl SearchCorpusGenerationV1 {
    pub fn new(
        lexical: GenerationSnapshot,
        semantic: GenerationSnapshot,
        semantic_content: SemanticContentRootsV1,
    ) -> Result<Self, CoreError> {
        if !semantic_content.is_canonical_v1() {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: semantic content roots must be canonical sha256 digests"
                    .to_string(),
            ));
        }
        if lexical.track != SearchPlaneTrackKind::Lexical
            || semantic.track != SearchPlaneTrackKind::Semantic
        {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: expected exactly lexical and semantic tracks"
                    .to_string(),
            ));
        }
        if lexical.repo_id != semantic.repo_id
            || lexical.revision_id != semantic.revision_id
            || lexical.manifest_generation != semantic.manifest_generation
            || lexical.manifest_digest != semantic.manifest_digest
        {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: lexical and semantic identities must match exactly"
                    .to_string(),
            ));
        }
        if lexical.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: manifest_digest must not be empty".to_string(),
            ));
        }
        Ok(Self {
            lexical,
            semantic,
            semantic_content,
        })
    }

    #[must_use]
    pub fn lexical(&self) -> &GenerationSnapshot {
        &self.lexical
    }

    #[must_use]
    pub fn semantic(&self) -> &GenerationSnapshot {
        &self.semantic
    }

    /// The content roots the semantic generation sealed (QI-BB-028).
    #[must_use]
    pub fn semantic_content(&self) -> &SemanticContentRootsV1 {
        &self.semantic_content
    }

    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.lexical.repo_id
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.lexical.revision_id
    }

    #[must_use]
    pub fn manifest_generation(&self) -> ManifestGeneration {
        self.lexical.manifest_generation
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        self.lexical.manifest_digest.as_str()
    }
}

/// A validated compare-and-swap promotion for one complete search corpus.
///
/// The only constructor requires a full lexical plus semantic identity.  This
/// makes lexical-only activation unrepresentable on the canonical path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSearchCorpusGenerationV1 {
    candidate: SearchCorpusGenerationV1,
    expected_active: Option<SearchCorpusGenerationV1>,
}

impl PreparedSearchCorpusGenerationV1 {
    pub fn new(
        candidate: SearchCorpusGenerationV1,
        expected_active: Option<SearchCorpusGenerationV1>,
    ) -> Result<Self, CoreError> {
        if let Some(expected) = &expected_active
            && (expected.repo_id() != candidate.repo_id()
                || expected.revision_id() != candidate.revision_id())
        {
            return Err(CoreError::InvalidContract(
                "search-corpus activation: expected active identity must match candidate repo and revision"
                    .to_string(),
            ));
        }
        Ok(Self {
            candidate,
            expected_active,
        })
    }

    #[must_use]
    pub const fn candidate(&self) -> &SearchCorpusGenerationV1 {
        &self.candidate
    }

    #[must_use]
    pub const fn expected_active(&self) -> Option<&SearchCorpusGenerationV1> {
        self.expected_active.as_ref()
    }
}

/// Exact receipt of a composite activation.  `previous_active` is retained
/// for a future typed rollback contract; it is absent for first activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationActivationV1 {
    pub active: SearchCorpusGenerationV1,
    pub previous_active: Option<SearchCorpusGenerationV1>,
}

/// Private on-disk activation-root record.
///
/// This deliberately does not reuse any public control DTO: persisted catalog
/// state is a complete lexical+semantic root, not an ingress request.
#[derive(Debug)]
pub(super) struct PersistedSearchCorpusGenerationRootV1 {
    lexical: GenerationSnapshot,
    semantic: GenerationSnapshot,
    semantic_content: SemanticContentRootsV1,
}

const PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS: &[&str] =
    &["lexical", "semantic", "semantic_content"];

impl PersistedSearchCorpusGenerationRootV1 {
    pub(super) fn from_generation(generation: &SearchCorpusGenerationV1) -> Self {
        Self {
            lexical: generation.lexical().clone(),
            semantic: generation.semantic().clone(),
            semantic_content: generation.semantic_content().clone(),
        }
    }

    pub(super) fn into_generation(self) -> Result<SearchCorpusGenerationV1, CoreError> {
        SearchCorpusGenerationV1::new(self.lexical, self.semantic, self.semantic_content).map_err(
            |err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: invalid composite root: {err:?}"
                ))
            },
        )
    }
}

impl Serialize for PersistedSearchCorpusGenerationRootV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PersistedSearchCorpusGenerationRootV1", 3)?;
        state.serialize_field("lexical", &self.lexical)?;
        state.serialize_field("semantic", &self.semantic)?;
        state.serialize_field("semantic_content", &self.semantic_content)?;
        state.end()
    }
}

struct PersistedSearchCorpusGenerationRootV1Visitor;

impl<'de> Visitor<'de> for PersistedSearchCorpusGenerationRootV1Visitor {
    type Value = PersistedSearchCorpusGenerationRootV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a persisted search-corpus generation root map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lexical = None;
        let mut semantic = None;
        let mut semantic_content = None;
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
                        PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(PersistedSearchCorpusGenerationRootV1 {
            lexical: lexical.ok_or_else(|| de::Error::missing_field("lexical"))?,
            semantic: semantic.ok_or_else(|| de::Error::missing_field("semantic"))?,
            semantic_content: semantic_content
                .ok_or_else(|| de::Error::missing_field("semantic_content"))?,
        })
    }
}

impl<'de> Deserialize<'de> for PersistedSearchCorpusGenerationRootV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PersistedSearchCorpusGenerationRootV1",
            PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS,
            PersistedSearchCorpusGenerationRootV1Visitor,
        )
    }
}

pub(super) fn search_corpus_generation_from_validated_rollback_identity(
    identity: &quanta_index_contract::SearchCorpusGenerationIdentityV1,
) -> Result<SearchCorpusGenerationV1, CoreError> {
    SearchCorpusGenerationV1::new(
        identity.lexical.clone(),
        identity.semantic.clone(),
        identity.semantic_content.clone(),
    )
}

pub(super) fn search_corpus_generation_into_contract(
    identity: &SearchCorpusGenerationV1,
) -> quanta_index_contract::SearchCorpusGenerationIdentityV1 {
    quanta_index_contract::SearchCorpusGenerationIdentityV1 {
        lexical: identity.lexical().clone(),
        semantic: identity.semantic().clone(),
        semantic_content: identity.semantic_content().clone(),
    }
}

pub(super) fn validate_prepared_search_corpus_expectation(
    prepared: &PreparedSearchCorpusGenerationV1,
    current: Option<&SearchCorpusGenerationV1>,
) -> Result<(), CoreError> {
    if prepared.expected_active() == current {
        return Ok(());
    }
    let describe = |identity: Option<&SearchCorpusGenerationV1>| {
        identity.map_or_else(
            || "absent".to_string(),
            |identity| {
                format!(
                    "generation={} digest={} row_root={} membership_root={}",
                    identity.manifest_generation().get(),
                    identity.manifest_digest(),
                    identity.semantic_content().row_root_digest,
                    identity.semantic_content().membership_root_digest,
                )
            },
        )
    };
    Err(CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CompositeActivationCasConflict,
        message: format!(
            "search-corpus activation: active composite root changed for repo={} revision={}: expected={}, observed={}",
            prepared.candidate().repo_id().as_str(),
            prepared.candidate().revision_id().as_str(),
            describe(prepared.expected_active()),
            describe(current),
        ),
    })
}
